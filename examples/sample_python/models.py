"""Data models for a simple user management system."""

from abc import ABC, abstractmethod
from dataclasses import dataclass, field
from datetime import datetime
from enum import Enum
from typing import Optional


class UserRole(Enum):
    """User role enumeration."""
    ADMIN = "admin"
    USER = "user"
    GUEST = "guest"


@dataclass
class Address:
    """Represents a physical address."""
    street: str
    city: str
    country: str
    postal_code: str

    def format(self) -> str:
        """Format the address as a single string."""
        return f"{self.street}, {self.city}, {self.postal_code}, {self.country}"


class BaseModel(ABC):
    """Abstract base class for all models."""

    @abstractmethod
    def to_dict(self) -> dict:
        """Convert the model to a dictionary."""
        pass

    @abstractmethod
    def validate(self) -> bool:
        """Validate the model data."""
        pass


@dataclass
class User(BaseModel):
    """Represents a user in the system."""
    id: int
    username: str
    email: str
    role: UserRole = UserRole.USER
    address: Optional[Address] = None
    created_at: datetime = field(default_factory=datetime.now)
    is_active: bool = True

    def to_dict(self) -> dict:
        """Convert user to dictionary representation."""
        return {
            "id": self.id,
            "username": self.username,
            "email": self.email,
            "role": self.role.value,
            "address": self.address.format() if self.address else None,
            "created_at": self.created_at.isoformat(),
            "is_active": self.is_active,
        }

    def validate(self) -> bool:
        """Validate user data."""
        if not self.username or len(self.username) < 3:
            return False
        if not self.email or "@" not in self.email:
            return False
        return True

    def deactivate(self) -> None:
        """Deactivate the user account."""
        self.is_active = False

    def activate(self) -> None:
        """Activate the user account."""
        self.is_active = True

    def change_role(self, new_role: UserRole) -> None:
        """Change the user's role."""
        self.role = new_role

    @classmethod
    def create_guest(cls, id: int) -> "User":
        """Create a guest user with minimal information."""
        return cls(
            id=id,
            username=f"guest_{id}",
            email=f"guest_{id}@example.com",
            role=UserRole.GUEST,
        )


class UserRepository:
    """Repository for managing user persistence."""

    def __init__(self):
        """Initialize an empty repository."""
        self._users: dict[int, User] = {}
        self._next_id = 1

    def add(self, user: User) -> User:
        """Add a user to the repository."""
        if user.id in self._users:
            raise ValueError(f"User with id {user.id} already exists")
        self._users[user.id] = user
        return user

    def get(self, user_id: int) -> Optional[User]:
        """Get a user by ID."""
        return self._users.get(user_id)

    def get_by_username(self, username: str) -> Optional[User]:
        """Get a user by username."""
        for user in self._users.values():
            if user.username == username:
                return user
        return None

    def get_all(self) -> list[User]:
        """Get all users."""
        return list(self._users.values())

    def get_active(self) -> list[User]:
        """Get all active users."""
        return [u for u in self._users.values() if u.is_active]

    def delete(self, user_id: int) -> bool:
        """Delete a user by ID."""
        if user_id in self._users:
            del self._users[user_id]
            return True
        return False

    def count(self) -> int:
        """Get the total number of users."""
        return len(self._users)

    def create(self, username: str, email: str, role: UserRole = UserRole.USER) -> User:
        """Create and add a new user."""
        user = User(
            id=self._next_id,
            username=username,
            email=email,
            role=role,
        )
        self._next_id += 1
        return self.add(user)
